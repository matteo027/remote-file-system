import { Entity, JoinTable, ManyToMany, PrimaryColumn } from "typeorm";
import { User } from "./User";

@Entity()
export class Group {
  @PrimaryColumn()
  groupname: string;

  @ManyToMany(() => User, (user) => user.groups)
  users: User[];
}